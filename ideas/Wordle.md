How would one efficiently compute the optimal strategy for a game of Wordle?

### Fundamentals

The game of Wordle relies on a definition of the following components:

- The number of characters in each word: $N_{c} = 5$.
- The set of all possible characters $L$, which are simply the letters A-Z.
- The set of all theoretically possible words $W = L^{N_{c}}$. This is simply any string of length $N_{c}$, with characters from $L$.
- The set of (remaining) candidates $C$. We will denote the initial (complete) set of candidates using $C_{0}$, such that $C \subseteq C_{0} \subseteq W$. 
- The set of all allowed words to guess $G$. In order for a game to always be winnable, we assume $C_{0} \subseteq G \subseteq W$.
- The set of all possible responses $R=\{b,g,y\}^{N_{c}}$. Where each response consists of the colors black $b$, green $g$ and yellow $y$. 
	- We denote the all green (win) response as $r_{w}=\{ g \}^{N_{c}}$ and denote the all black response as $r_{b}=\{ b \}^{N_{c}}$.
- The response function $f: (C_{0} \times G) \to R$, that maps each candidate $c \in C_{0}$, guess $g \in G$ pair to a corresponding response $r \in R$ that would be shown by Wordle if $c$ was the secret word and you guessed $g$.

We assume the following:
- each candidate word is equally likely to be the secret word.
- there is no limit on the number of guesses, as this simplifies calculations later on. This assumption is of course false, but I since any reasonable heuristic is capable of never exceeding the limit of $6$ guesses, the optimal strategy will never do so aswell. Consequently making this limit irrelevant.
- we are playing the game in "easy mode", meaning we get to pick from the entire list of guesses every time, instead of just the list of remaining candidates (hard mode).

### Response Function

There are many equivalent ways to determine the output of the response function. The pseudo-code for one such way is provided as follows.

```plaintext  
function compute_response(secret, guess):  
    N_c = length(secret)    
    assert N_c == length(guess)    
    response = array(N_c, BLACK)    
    used = array(N_c, false)        
    
    // Pass 1: find greens  
    for i in 0..N_c-1:        
        if guess[i] == secret[i]:            
            response[i] = GREEN            
            used[i] = true        
    
    // Pass 2: find yellows  
    for i from 0..N_c-1:        
        if response[i] == GREEN:            
            continue        
        for j in 0..N_c-1:            
            if guess[i] == secret[j] AND !used[j]:
                response[i] = YELLOW
                used[j] = true                
                break        
    
    return response  
```  
  
Intuitively, the total number of distinct responses is derived as $|R| = |\{b,g,y\}|^{N_c} = 3^{5} = 243$. However, some of these responses are impossible, specifically the $N_{c}$ responses where $N_c - 1$ of the letters are green, and $1$ is yellow. This results in theoretically maximal number of $243-5=238$ responses. 

Note: If a guess contains a letter that does not occur in the secret word, it is guaranteed to get a black response. However, this does not mean that if a specific letter gets a black response, it does not occur in the secret word. As the guess word may just contain the letter more frequently than the secret word does.

### Partitions  
  
Given a guess $g \in G$, we may **partition** our candidate set $C$ into disjoint subsets $C_{g,r}$, where $C_{g,r}$ is the set of candidates $c \in C$ that would generate response $r$ if $g$ were guessed.  

$$
C_{g,r} = \{c \in C \mid f(c, g) = r\}
$$  
  
Properties:   
1. $C_{g,r} \subseteq C$
2. $r_1 \neq r_2 \implies C_{g,r_1} \cap C_{g,r_2} = \emptyset$  
3. $g \in C_{g,r} \implies C_{g,r} = \{g\} \text{ and } r = r_{w}$.  
4. $C_{g,r} = C_{0,g,r} \cap C$ 

### Useless Guesses

We say a guess $g$ is **useless** with respect to a non-empty candidate set $C$ if there exists a specific, **singular response** $r^{\ast} \in R$ such that $C_{g,r^{\ast}}=C$. That is, $g$ fails to partition $C$.

Properties:
1. Any guess $g$ is useless with respect to any singleton candidate set $C$.
	$$
	|C| = 1 \implies g \text{ is useless w.r.t. C}
	$$
2. For any useless guess $g$, with singular response $r^{\ast}$ for some candidate set $C$, all other responses yield the empty set. 
	$$
	\forall r \in R: \left( r \neq r^{\ast} \implies C_{g,r}=\emptyset \right)
	$$
3. If a guess $g$ is in the candidate set ($g \in C$), and there is more than one candidate remaining ($|C|>1$), then $g$ is not useless.
4. For any candidate set $C$, any guess $g$ can only be useful at most once. Any guess after the first will just yield the same response for all candidates.
	$$
	\forall g \in G: \left( C' \subseteq C_{g,r^{\ast}} \implies C'_{g,r^{\ast}}=C' \right)
	$$
5. If a guess $g$ is useless with respect to $C$, it will also be useless with respect to any nonempty subset of $C$, yielding the same singular response $r^*$.
	$$
	\forall g \in G : (C_{g,r^*}=C \implies \forall C' \subseteq C:(C'_{g,r^*}=C'))
	$$
6. If a guess $g$ shares no letters with any candidate in $C$, then it is useless, and its singular response is the all-black response $r_{b}$.
	$$
	\forall c \in C : \left( \text{letters}(g) \cap \text{letters}(c) = \emptyset \right) \implies C_{g, r_{b}} = C
	$$
6. A guess $g$ is useful with respect to $C$ if and only if it strictly reduces the size of the candidate space. 
	$$
	\neg\exists r \in R: (C_{g,r} = C) \implies \forall r \in R: (|C_{g,r}|<|C|)
	$$

### Equivalent Guesses

We say a guess $g_{1}$ is **equivalent to** another guess $g_{2}$ with respect to a candidate set $C$ (denoted $g_{1} \equiv_{C} g_{2}$), if they produce the same partitions. 

$$
g_{1} \equiv_{C} g_{2} \iff \forall r \in R :C_{g_{1},r}= C_{g_{2},r} \iff \forall c \in C :f(c,g_{1}) = f(c,g_{2})
$$

Properties:
1. If two guesses $g_{1}$ and $g_{2}$ are equivalent for a candidate set $C$, then they are also equivalent for any subset of $C$. Note the inverse does not hold.
	$$
	\forall C' \subseteq C :(g_{1} \equiv_{C} g_{2} \implies  g_{1} \equiv_{C'} g_{2})
	$$
2. Guess equivalence forms an equivalence relation on the set of guesses $G$ with respect to a candidate set $C$. This means the relation $\equiv_{C}$ satisfies the following three properties for all $g, g_{1}, g_{2}, g_{3} \in G$:
	- Reflexivity: $g \equiv_{C}g$
	- Symmetry: $g_{1} \equiv_{C} g_{2} \implies g_{2} \equiv_{C} g_{2}$.
	- Transitivity: $(g_{1} \equiv_{C} g_{2} \land g_{2} \equiv_{C} g_{3}) \implies g_{1} \equiv_{C} g_{3}$

Next, we would like to determine an efficient way to check whether two guesses are equivalent. First, we define $L_{C}$ as the subset of letters $L_{C} \subseteq L$ that appear in at least one candidate $c \in C$.

$$
L_{C} = \{ l \in L \mid \exists c \in C: l \in c\}
$$

Next we recognize that for any letter $l \in L \setminus L_{C}$, any guess $g$ containing this letter is guaranteed to get a black response in the corresponding position. This means these letters are interchangeable with respect to $C$. To utilize this, we define the projection function $P_{C}(g)$ that replaces any letter in $g$ not found in $L_{c}$ with a generic wildcard (e.g. $*$). If the projections of two guesses are equal, then the guesses are guaranteed to be equal aswell.

$P_{C}(g_{1}) = P_{C}(g_{2}) \implies g_{1} \equiv_{C} g_{2}$

One could attempt to refine $P_{C}$ to turn this implication into a biconditional, but this would likely be no more efficient then evaluating $g_{1} \equiv_{C} g_{2}$ directly.

### As a Markov Decision Process  
  
Using the aforementioned definitions, the full game of Wordle may be defined as an infinite horizon Markov Decision Process (MDP) as follows.  
  
Trivially, each state is just the set of remaining candidate words $C$. The set of all states then becomes the power set of the initial candidate set $\mathcal{P}(C_{0})$. However, we must add a terminal win state for when the secret word has been guessed correctly. We recognize the state with "no words remaining" is always unreachable, this essentially frees up the empty state $\emptyset \in \mathcal{P}(C_{0})$ which we will use to denote the win state instead.  

The transition function $P(C' \mid C, g)$ defines the probability of transitioning to a next state $C'$ given current state $C$ and guess $g$. 

$$
P(C' \mid C, g) = \begin{cases} 
1 & \text{if } C = \emptyset, C' = \emptyset \\
\frac{1}{|C|} & \text{if } C \neq \emptyset, C' = \emptyset, g \in C \\
\frac{|C' \setminus \{g\}|}{|C|} & \text{if } C \neq \emptyset, C' = C_{g,r} \\
0 & \text{otherwise} 
\end{cases}
$$

We define the cost function $Cost(C)$, and discount factor $\gamma=1$ such that the total cost corresponds to the total number of guesses made until the secret word was guessed.  
  
$$
Cost(C) = \begin{cases} 0 & \text{if } C = \emptyset \\ 1 & \text{otherwise} \end{cases}
$$  
  
The goal is to find an optimal policy, $\pi^{\ast}: \mathcal{P}(C_{0}) \to G$, that minimizes the expected total cost (expected total number of guesses).  
  
Substituting these definitions into the Bellman optimality equations and simplifying gives:  

$$
Q^{\ast}(\emptyset,\cdot) = 0
$$

$$
Q^{\ast}(C, g) = 1 + \sum_{r \in \hat{R} } \frac{|C_{g,r}|}{|C|} V^{\ast}(C_{g,r})
$$  
  
$$
V^{\ast}(C) = \min_{g \in G} Q^{\ast}(C, g)
$$  
  
Where $\hat{R} = \{r \in R   \mid r \neq r_w \}$ is the set of all possible responses excluding the win response.  

### As a Partially Observable MDP

(TODO: explain that we can model it as a POMDP, and explain the equivalence with the aforementioned MDP)

### The Infeasibility
  
The reason we can't compute the optimal strategy efficiently is because the number states is exponential in the number of initial candidates. 
  
$$
|\mathcal{P}(C_{0})| =2^{|C_0|}
$$
  
In order to make the algorithm feasible, we must drastically reduce the number of states to visit.  
  
A first insight is to realize that not all states are reachable from $|C_0|$. However, an attempt to find all reachable states via a BFS traversal will quickly show that even this simplified problem is still intractable, showing the same exponential growth.

A next insight is then, that many of these states are only reachable by making terrible guesses that clearly don't correspond to the optimal value. For example, a guess filtering out only one candidate will lead to a new distinct state, but this state is likely irrelevant to the optimal strategy.
  
### Integer optimization 
  
To avoid the computational overhead and precision issues of floating point arithmetic, we reformulate the objective function. Instead of minimizing the expected number of guesses (which requires division), we minimize the total number of guesses required to solve for all candidates in $C$.  
  
Let $T^{\ast}(C)$ be the minimum total guesses for candidate set $C$. We can define the relationship to the expected value $V^{\ast}(C)$ as:  
  
$$
T^{\ast}(C) = |C| \cdot V^{\ast}(C)
$$  
  
We can then update the Bellman Equations as follows:  
  
$$
T^{\ast}(C) =  \min_{g \in G} \left( |C| \cdot 1 + |C| \sum_{r \in \hat{R}} \frac{|C_{g,r}|}{|C|} V^{\ast}(C_{g,r}) \right) = \min_{g \in G} \left( |C| + \sum_{r \in \hat{R}} T^{\ast}(C_{g,r}) \right)
$$  
  
This gives the integer-only recurrence relation. Note that the term $|C|$ represents the fact that the current guess $g$ adds exactly 1 guess to the path of every candidate currently in the set.  
  
Rewriting the recurrence relation in alternating recursive form we get:  

$$
T^{\ast}(C) = \min_{g \in G} T^{\ast}(C, g)
$$  
  
$$
T^{\ast}(C, g) = |C| + \sum_{r \in \hat{R}} T^{\ast}(C_{g,r})
$$  
  
We recognize the following base cases:
  
- Empty Set: $T^{\ast}(\emptyset) = 0$  
- Single Word: $T^{\ast}(\{c\}) = 1$   
  - The word is guessed immediately.  
- Two Words: $T^{\ast}(\{c_1, c_2\}) = 3$.  
  - 1 guess identifies the first word.  
  - 2 guesses identifies the second.
  
### Memoization  
  
We use memoization store $T^{\ast}(C)$ for each set of candidates $C$ where $|C| > 2$ (it is not a base case). We use top-down dynamic programming instead of bottom-up dynamic programming since bottom-up cannot utilize the large amount of pruning we intend to do.

### Branch and Bound  
  
We use a Branch and Bound strategy to prune guesses that cannot possibly yield an optimal solution. This branch and bound algorithm is abstractly defined as follows:

`min_state_val`: Given an upper bound $\beta$ and a state $C$, it returns $\beta$ if $T^{\ast}(C) \geq \beta$, and $T^{\ast}(C)$ otherwise.

1. (Optional): Sort/filter the guesses
2. For each guess $g$:
	1. $\beta := \texttt{min\_guess\_val}(C, g, \beta)$
3. (Optional): For $C$ cache best guess and $T^{\ast}(C)$ if we managed to beat $\beta$.
4. Return $\beta$

`min_guess_val`: Given an upper bound $\beta$, a state $C$ and a guess $g$, it returns $\beta$ if $T^*(C,g) \geq \beta$ and $T^{\ast}(C,g)$ otherwise. 

1. Compute each of the partitions $C_{g,r}$ for the given guess $g$.
2. If there is only a single partition, the guess was useless, so we return $\beta$.
3. Determine the initial lower bound $T_{LB}(C,g)$ using the (size of the) partitions.
4. If $T_{LB}(C, g) \geq \beta$, return $\beta$. 
5. For each partition $C_{g,r}$, we refine its contribution to the lower bound:
	1. Get exclusive lower bound $b := T_{LB}(C,g)-T_{LB}(C_{g,r})$
	2. $T_{LB}(C,g) := b + \texttt{min\_state\_val}(C_{g,r}, \beta - b)$
	3. If $T_{LB}(C, g) \geq \beta$, return $\beta$. 
6. At this point $T_{LB}(C,g) < \beta$, and $T_{LB}(C,g)=T^{\ast}(C,g)$, so we return $T_{LB}(C,g)$.

Finally to determine $T^{\ast}(C_{0})$ we first determine some initial upper bound $UB(C_{0})$, and then call the $\texttt{min\_state\_val}(C_{0}, UB(C_{0}))$.

### Maintaining useful guesses  
  
We observe that as the set of candidates $C$ shrinks, the set of useful guesses in $G$ also shrinks. Consequently, iterating over the full set of all allowed guesses (approx. 13,000) is wasteful when $|C|$ is small, as most guesses will yield zero information.

In order to solve this, we can maintain a reduced list of potentially useful guesses $\hat{G}$ to pass down the recursion tree. Note this set **must** be a superset of the truly useful guesses. By excluding guesses that could be capable of partitioning $C$ we lose the guarantee of optimality.

On top of this we can sort the guesses based on how "useful" they are expected to be. This can be done using any of the heuristic scores. Interestingly enough, determining whether a guess has "no common letters" with any of the remaining words, or directly determining whether it is capable of partitioning $C$, combine very well with computing the heuristic scores for letter frequencies, or min expected remaining respectively.

### Max distinct responses   
  
We want to know the maximum number of candidates that can be distinguished in a single step.  
  
Let $\rho(C, g)$ be the number of distinct responses generated by a guess $g$ against a candidate set $C$:  
  
$$
\rho(C, g) = |\{ r \in R \mid \exists c \in C, f(c, g) = r \}|
$$  
  
We define the Max Branching Factor $K(C)$ as the maximum possible distinct responses achievable by any single guess:  
  
$$
K(C) = \max_{g \in G} \rho(C, g)
$$  

The combinatorial limit for this max branching factor was previously determined to be $238$. The practical limit is lower than this, specifically due to correlations between English words, no single guess can actually achieve 238 partitions.
  
Computing $K(C)$ is an expensive operation ($O(|G|\cdot|C|)$). However, we rely on the property that the branching factor is monotonic with respect to set inclusion. If $C' \subseteq C$, then for all $g$, $\rho(C', g) \le \rho(C, g)$. Consequently:  
  
$$
K(C') \le K(C)
$$ 
  
This implies the max branching factor of the initial set of candidates $K_{0} = K(C_0)$ is larger than or equal to any other max branching factor $K(C)$.  

### Lower bounds on $T^{\ast}(C)$  
  
We want admissible lower bounds (optimistic estimates) on the cost to perform pruning.
  
#### Bound 1 (Information Theoretic)

Each response from Wordle provides information that reduces the candidate set. To distinguish among $|C|$ possibilities requires at least $\log_{K(C_{0})}(|C|)$ responses in expectation. Converting this to total guesses:  
  
$$
T^{\ast}(C) \geq |C| \cdot \log_{K(C_{0})} |C| = |C| \cdot \log_2 |C| \cdot \gamma \geq |C| \cdot \lfloor \log_{2}|C| \rfloor \cdot \gamma 
$$  
  
Where $\gamma = 1 / \log_2(K(C_{0}))$ is a constant independent of $C$, and $\lfloor \log_{2}|C| \rfloor$ can be computed extremely efficiently on most computers.
  
#### Bound 2 (Minimum Depth / Pigeonhole)

We derive the absolute minimum number of total guesses required for a set of size $|C|$ by assuming the best-case scenario for every guess.   
1. We must pick a single guess $g$.   
2. If the secret word happens to be $g$ we solve it in 1 guess. This happens for at most one candidate in $C$.    
3. For the remaining $|C| - 1$ candidates, the guess $g$ is incorrect. Therefore, we need at least 1 additional guess to solve them. Giving a path length of at least 2.  
  
$$T^{\ast}(C) \geq 1 \cdot 1 + 2 \cdot (|C| - 1) = 2|C| - 1$$  
  
#### Bound 3 (Capacity Bound Pigeonhole)

Using the branching factor $K$ we can tighten the lower bound for large sets ($|C| > K$). The standard Pigeonhole bound assumes we can solve all remaining candidates at Depth 2. However, we are physically limited by the number of distinct buckets (responses) available.   
  
4. Depth 1: We can identify at most 1 candidate (the secret word itself).  
5. Depth 2: We can identify at most $K-1$ distinct candidates. (One distinct response per candidate, minus the "Win" response used at Depth 1).  
6. Depth 3: Any remaining candidates must be solved at Depth 3 or greater.  
  
Derivation: If $|C| > K$, the minimum configuration of guesses is:  
- $1$ candidate at cost 1. 
- $K-1$ candidates at cost 2.  
- The remaining $|C| - 1 - (K - 1) = |C| - K$ candidates at cost 3 (optimistically).  
  
$$
T^{\ast}(C) \geq 1(1) + 2(K - 1) + 3(|C| - K) =  3|C| - K - 1
$$  
  
Note: While this geometric capacity constraint extends to deeper levels (Depth 4, 5, etc.), for standard Wordle the remaining candidates always fit within Depth 3 (since $K^2 > ∣C∣$), making further expansion unnecessary.  

#### Combining Lower Bounds

The tightest lower bound $LB(C)$ is just the maximum of all lower bounds. Assuming $N_c = 5$, Bound 2 is tighter than bound 1 for all relevant candidate set sizes. On top of this, its also more efficient to compute. Bound 3 is tighter than bound 2 $|C| > K$, but slightly less efficient to compute.

#### Lower bounds on $T^{\ast}(C,g)$

Finally, we may compute a lower bound on the specific total cost of a guess $T^{\ast}(C,g)$ (with $C \neq \emptyset$) as follows:

$$
T_{LB}(C, g) = |C| + \sum_{r \in \hat{R} } \hat{T}(C_{g,r})
$$  
  
where:  
  
$$
\hat{T}(C) = \begin{cases} 1 & \text{if }|C|=1  \\
3& \text{if }|C| = 2 \\
T^{\ast}(C) & \text{if }C \text{ in cache} \\ LB(C) & \text{otherwise} \end{cases}
$$
  
### Upper bounds on $T^{\ast}(C)$  
  
Since we are minimizing cost, the total cost $T^h(C_{0})$ produced by any valid policy $h$ is a valid upper bound $UB(C_{0})$ on the true minimal total cost $T^{\ast}(C_{0})$.  

Let $h: \mathcal{P}(C_{0}) \to G$ be a heuristic policy function that returns a guess $g$ for a candidate set $C$. We can calculate the total cost by simulating the game tree using $h$. This is computed as:

$$
T^{h}(C) = |C| + \sum_{r \in \hat{R}} T^{h}(C_{h(C),r})
$$

$$
T^{h}(\emptyset) = 0
$$ 

We use this $UB(C)$ as the first upper bound $\beta$. We then repeatedly refine this upper bound as we go on.

### Heuristics  
  
#### Pick Max Frequency  
  
**Goal**: Maximize coverage of common letters across candidates.  
  
First we define the number of candidate words $c \in C$ containing a specific letter $l$:  
  
$$
\text{freq}(l) = |\{c \in C \mid l \in c\}|
$$  
  
For each guess $g$, we sum the frequencies across the unique letters from $g$.
  
$$
\text{score}(g) = \sum_{l \in \text{unique}(g)} \text{freq}(l)
$$
- where $\text{unique}(g)$ is the set of distinct letters in $g$.   

Computing the score of all guesses has a time complexity of  $O(|G|+|C|+|L|)$.

The approximation of the optimal guess is the guess that maximizes the score:  
$$
g^{\ast} = \arg\max_{g \in G} \text{score}(g)
$$

#### Pick Min Remaining  
  
**Goal**: Minimize expected number of remaining candidates after the guess.  
  
Expected remaining candidates for a given guess $g$:  
  
$$
\mathbb{E}[|C_{g,r}|] = \sum_{r \in R} \frac{|C_{g,r}|}{|C|} \cdot |C_{g,r}|= \frac{1}{|C|} \sum_{r \in R} |C_{g,r}|^2
$$  
  
$|C|$ is independent of the guess, so we leave it out in the score.  
  
$$
\text{score}(g) = \sum_{r \in R} |C_{g,r}|^2
$$  

Computing the score of all guesses has a time complexity of $O(|G|\cdot|C|)$.

The approximation of the optimal guess is the guess that minimizes the score:  
  
$$
g^{\ast} = \arg\min_{g \in G} \text{score}(g)
$$

### Algorithmic Optimizations

#### Indices

Instead of storing actual letters, responses, and words, it is beneficial to store the indices of their respective collections instead. This saves space, and leads to more efficient serialization or lookups. 

To determine the ordering of the collections we simply sort the set alphabetically, and then assign assign an index to each element based on this ordering.  
- For guesses and candidates, ordering alphabetically is trivial.
- For the responses, we interpret a response as a $N_{c}$ length string, where each of the symbols in the string correspond to black $b$, green $g$, or yellow $y$. These strings are then sorted alphabetically. Equivalently, a response string may be efficiently mapped to an index by mapping the letters to numbers $b \to 0, g\to{1}, y\to 2$ and reading the string as a little-endian ternary number.
- For letters A-Z, we can map a letter to its index by simply subtracting the letter A from all of the letters, consequently assigning the numbers 0-25 to the letters. 

#### Response Cache

The solver will rely heavily on partitioning candidate sets based on responses. As a consequence the speed with which we can determine the response corresponding to a given secret word, and guess is important. To solve this we precompute a $|G| \times |C_{0}|$ (rows x columns) response matrix, stored in a single vector in row-major order (to improve cache locality). Indexed using the combined indices of the guess and candidate, and containing the corresponding response indices. 

#### Vector Maps

If the maximum index is quite small (as is the case with the letters, and responses), one may also replace hash sets/maps with a vector or bitset, removing the need for hashing entirely, at the cost of allocating a little more space for sparse sets. 

This cost of allocation can be remediated by reusing the same set multiple times, in such a case you maintain two vectors, one functioning as the set/map, and the other listing the indices from the first vector that are non empty. Assigning values stays O(1), but clearing the vector takes O(n) instead of O(capacity), where capacity is the maximum index + 1.

For memory efficiency, it is likely that the first vector (the bitset) will be a bit-set. In such as case the O(n) reset operation is likely to be slower for dense sets than the O(capacity) operation. In this scenario one might consider a hybrid approach, where the vector map switches to bitset level clearing instead of the backtracking approach whenever the number of elements in the set exceeds some number. 

In general, the most significant bottlenecks will be the way we represent candidates, and specifically how we partition these candidates. In order to compare the aforementioned methods, it is recommended to have the solver rely on an interface. In this case we can swap the backends easily, allowing the different approaches to be compared.

#### Hashing

If one is to rely on some sort of HashMap where the keys are based on the response, letter, guess, or candidate indices, you may benefit from using a simple hasher. Specifically Hash-DoS protection is redundant, and is included in the default Rust hasher.

#### Recursion vs Iteration

The branch and bound definitions define a recursive approach. At the cost of readability, one may consider implementing the logic in a single iterative function to reduce the overhead of function calls. 

### Policy Evaluation

We define two abstractions:
- A **response policy**, which must implement a method named `guess` that accepts an optional response, and returns the next action or an error if the response was unexpected. If no response was provided, this indicates the policy must provide its first guess. 
- A **candidates policy**, which must implement a method named `guess` that accepts the set of remaining candidates, and returns the next action, or an error if the remaining candidate set was unexpected. 

A candidate set, or a history of previous guesses and their respective responses is a full description of the current state, however a single response (e.g. `bbgyy`) is not. Consequently, any response policy is responsible for keeping track of the state itself, whereas any candidates policy does not need to store any state.

Next we define a simple simulate function that relies on a response policy.
```plaintext
function simulate(policy, secret):
    guess := policy.guess(none)
    n_guesses := 0
    while guess != secret:
        response := compute_response(secret, guess)
        guess := policy.guess(response)
        n_guesses := n_guesses + 1
    return n_guesses
```

In order to utilize the same logic for any candidates policy, we write some wrapper logic (that implements the response policy abstraction) that maintains the set of remaining candidates internally, and passes this down to the candidates policy.

Next we define an evaluate function to determine the total number of guesses for a policy to guess all the candidates.
```plaintext
function evaluate(policy, initial_candidates):
	total_guesses := 0
	for secret in initial_candidates:
		total_guesses := total_guesses + simulate(policy, secret)
	return total_guesses
```

The expected number of guesses is then equal to `total_guesses` divided by the number of elements in `initial_candidates`. 

### Storing Policies

Because a policy is deterministic, its execution for a given set of initial candidates and allowed guesses can be perfectly modelled using a static decision tree. We do not need to store the dynamic shrinking list of remaining candidates at each step, the structure of the tree itself implicitly encodes the state based on the history of responses.

Any policy is represented using a tree, where:
- Guesses are nodes, with the initial guess at the root, and the leaf nodes being the secret word. 
- Edges are the responses returned by the game. 

While the theoretical state space of all possible guess-response combinations is massive, we only need to store the states reachable by strictly following the policy.

Any such policy tree may be serialized in two ways:
- compact: maintaining the usage of indices to indicate guesses, and responses.
- readable: as a JSON file, where the each node object has two attributes "guess", which is just a string representing the guess to make, and "children" which is a map of each response string to another node.

Any serialized policy tree must be accompanied by a hash of the alphabetically sorted set of allowed guesses and set of initial candidates. This hash can be checked to ensure the policy was created on the same set of initial candidates and allowed guesses.

Next we want an efficient way to determine these policy trees. First we define a separate simulate method (or rewrite the previous one) such that it returns the full history of guess response pairs given a response policy, and a secret word.
```plaintext
function simulate(policy, secret):
    history := []
    guess := policy.guess(none)
    
    while guess != secret:
        response := compute_response(secret, guess)
        history.append((guess, response))
        guess := policy.guess(response)
        
    // Append the final winning guess
    // The last response is always the win response (indicated by none).
    history.append((guess, none))
    
    return history
```

The policy tree of a response policy may then be determined as follows:
```plaintext
function capture(policy, initial_candidates):
	tree := {guess: none, children: {}}
	
	for secret in initial_candidates:
		history := simulate_history(policy, secret)
		node := tree
		
		for turn in history:
			if node.guess is not none and node.guess != turn.guess:
				throw Error("Policy is not deterministic!")
			node.guess := turn.guess
			
			if turn.response is none:
				break
			
			if turn.response not in node.children:
				node.children[turn.response] := {guess: none, children: {}}
			node := node.children[turn.response]
	
	return tree
```

Once such a tree has been captured, we can build some wrapper logic that implements the response policy abstraction for any given policy tree.