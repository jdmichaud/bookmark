# pyhon3 -mvenv venv
# source venv/bin/activate
# pip install -U sentence-transformers beautifulsoup4
import sys
import json
import os.path

import multiprocessing
import concurrent.futures

def extract_text(html):
  from urllib.request import urlopen
  from bs4 import BeautifulSoup

  soup = BeautifulSoup(html, features="html.parser")

  # kill all script and style elements
  for script in soup(["script", "style"]):
      script.extract()    # rip it out

  # get text
  text = soup.get_text()

  # break into lines and remove leading and trailing space on each
  lines = (line.strip() for line in text.splitlines())
  # break multi-headlines into a line each
  chunks = (phrase.strip() for line in lines for phrase in line.split("  "))
  # drop blank lines
  text = '\n'.join(chunk for chunk in chunks if chunk)

  return text

from sentence_transformers import SentenceTransformer
model = SentenceTransformer('sentence-transformers/all-MiniLM-L6-v2')

def compute_embeddings_string(str):
  return model.encode(str)

def compute_embeddings(filename):
  embeddings_filename = f'{filename}.embeddings'
  if os.path.isfile(embeddings_filename):
    print(f"embeddings already exists {embeddings_filename}")
  else:
    with open(filename, 'r') as file:
      content = extract_text(file.read())
      embeddings = compute_embeddings_string(content)
      with open(embeddings_filename, 'wb') as embeddings_file:
        l = embeddings.tolist()
        embeddings_file.write(bytes(json.dumps(l), 'utf-8'))
        print(f'{embeddings_filename} written')

def compute(argv):
  if (len(argv) < 1):
    print("expecting at least one arguments")
    sys.exit(1)

  executor = concurrent.futures.ProcessPoolExecutor(multiprocessing.cpu_count() - 2)
  futures = [executor.submit(compute_embeddings, filename) for filename in argv]
  concurrent.futures.wait(futures)

def search(argv):
  if (len(argv) < 3):
    print("expecting at least 3 arguments")
    sys.exit(1)

  needle, files = argv[0], argv[1:]
  needle_embeddings = compute_embeddings_string(needle)

  embeddings = dict()
  for filename in files:
    with open(filename, 'r') as file:
      embeddings[filename] = json.loads(file.read())

  from numpy import dot
  from numpy.linalg import norm

  results = []
  for file in files:
    a = needle_embeddings
    b = embeddings[file]
    cos_sim = dot(a, b) / (norm(a) * norm(b))
    results.append([cos_sim, file])

  results = sorted(results, key=lambda x: -x[0])
  for result in results[:5]:
    print(f'{result[0]} {result[1]}')


def similarity(argv):
  if (len(argv) < 2):
    print("expecting at least one arguments")
    sys.exit(1)

  embeddings = dict()
  for filename in argv:
    with open(filename, 'r') as file:
      embeddings[filename] = json.loads(file.read())

  from itertools import combinations
  combinations = [com for com in combinations(argv, 2)]

  from numpy import dot
  from numpy.linalg import norm

  for combination in combinations:
    a = embeddings[combination[0]]
    b = embeddings[combination[1]]
    cos_sim = dot(a, b) / (norm(a) * norm(b))
    print(f'{cos_sim} {combination[0]} {combination[1]}')

if __name__ == '__main__':
  if (sys.argv[1] == "compute"):
    compute(sys.argv[2:])
  elif (sys.argv[1] == "similarity"):
    similarity(sys.argv[2:])
  elif (sys.argv[1] == "search"):
    search(sys.argv[2:])
  else:
    print(f"error: command must be compute or similarity, not {sys.argv[1]}")
